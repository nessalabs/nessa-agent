/**
 * Where panes sit: columns side by side, each a stack of panes, one pane
 * focused. A value: every operation returns a new layout and leaves the one
 * it was given alone, and returns that same layout when nothing changes, so a
 * caller can tell a refused operation by identity.
 *
 * ```text
 *   columns ─▶ [ column { share, panes ─▶ [ pane { key, item, share } ] } ]
 * ```
 *
 * Each pane shows an item: an opaque id the host gives meaning to (a
 * conversation, a document). A share is a pane's (or column's) part of
 * its parent's room, relative to its siblings, so resizing the window keeps
 * proportions. Pane keys are minted by the layout itself and never reused
 * within it; a key names a pane for as long as it lives, whichever item it
 * shows and wherever it moves.
 *
 * The limits — how many panes and columns, and the smallest readable pane —
 * are data in `paneLimits`; the rules that need measured pixels take them from
 * `pane-sizing.ts`.
 */

export type PaneKey = number

/** One pane: the item it shows and its share of its column's height. */
export interface Pane {
  readonly key: PaneKey
  readonly item: string
  readonly share: number
}

/** One column: its share of the grid's width and its panes, top to bottom. */
export interface Column {
  readonly key: number
  readonly share: number
  readonly panes: readonly Pane[]
}

export interface PaneLayout {
  readonly columns: readonly Column[]
  readonly focused: PaneKey
  /** The next key to mint, for a pane or a column. */
  readonly nextKey: number
}

/** A side of a pane something can be placed on. */
export type Side = "left" | "right" | "top" | "bottom"
/** Where a drop lands: a side, or the pane itself. */
export type Zone = Side | "center"
/** The way a pane is nudged by the keyboard. */
export type Direction = "left" | "right" | "up" | "down"

export const paneLimits = {
  /** Past four panes nothing is readable at a laptop's width. */
  maxPanes: 4,
  maxColumns: 3,
  /** The narrowest a pane may be and still hold a line of a reply. */
  minWidth: 300,
  /** The shortest a stacked pane may be and still show a turn and a composer. */
  minHeight: 220,
  /** The gap between panes, as the stylesheet draws it. */
  gutter: 8,
} as const

export const isHorizontal = (side: Side): side is "left" | "right" =>
  side === "left" || side === "right"

/** A layout of one pane, showing `item`. */
export function singlePane(item: string): PaneLayout {
  return {
    columns: [{ key: 0, share: 1, panes: [{ key: 1, item, share: 1 }] }],
    focused: 1,
    nextKey: 2,
  }
}

/** Every pane, in reading order: down each column, then across. */
export function panesOf(layout: PaneLayout): readonly Pane[] {
  return layout.columns.flatMap((column) => column.panes)
}

export function paneCount(layout: PaneLayout): number {
  return layout.columns.reduce((count, column) => count + column.panes.length, 0)
}

/** Whether the layout holds as many panes as it may: a new one has nowhere to go. */
export function isFull(layout: PaneLayout): boolean {
  return paneCount(layout) >= paneLimits.maxPanes
}

/** Where a pane is: its column's index and its index in that column. */
export function locate(
  layout: PaneLayout,
  key: PaneKey,
): { column: number; row: number } | null {
  for (let column = 0; column < layout.columns.length; column++) {
    const row = layout.columns[column].panes.findIndex((pane) => pane.key === key)
    if (row >= 0) return { column, row }
  }
  return null
}

export function paneByKey(layout: PaneLayout, key: PaneKey): Pane | undefined {
  return panesOf(layout).find((pane) => pane.key === key)
}

/**
 * The pane showing an item, if one does: an item is never shown twice
 * (`showInPane` and `splitPane` hold that; `pane-layout.test.ts`).
 */
export function paneShowing(layout: PaneLayout, item: string): Pane | undefined {
  return panesOf(layout).find((pane) => pane.item === item)
}

/** The focused pane; a layout always has one. */
export function focusedPane(layout: PaneLayout): Pane {
  return paneByKey(layout, layout.focused) ?? panesOf(layout)[0]
}

export function focusPane(layout: PaneLayout, key: PaneKey): PaneLayout {
  if (layout.focused === key || !locate(layout, key)) return layout
  return { ...layout, focused: key }
}

function mapPanes(layout: PaneLayout, change: (pane: Pane) => Pane): PaneLayout {
  let changed = false
  const columns = layout.columns.map((column) => {
    let columnChanged = false
    const panes = column.panes.map((pane) => {
      const next = change(pane)
      if (next !== pane) columnChanged = true
      return next
    })
    if (!columnChanged) return column
    changed = true
    return { ...column, panes }
  })
  return changed ? { ...layout, columns } : layout
}

/**
 * Shows an item in a pane, in place of what it showed, and focuses it. An
 * item is never shown twice: one another pane shows already is focused where
 * it is instead, and `key` is left as it was.
 */
export function showInPane(layout: PaneLayout, key: PaneKey, item: string): PaneLayout {
  const existing = paneShowing(layout, item)
  if (existing) return focusPane(layout, existing.key)
  if (!locate(layout, key)) return layout
  const shown = mapPanes(layout, (pane) => (pane.key === key ? { ...pane, item } : pane))
  return focusPane(shown, key)
}

/**
 * Takes a pane out; the neighbour beside it takes its room, and focus moves
 * to the next pane in reading order, or the previous one. The last pane is
 * never removed: a layout always shows something.
 */
export function removePane(layout: PaneLayout, key: PaneKey): PaneLayout {
  const at = locate(layout, key)
  if (!at || paneCount(layout) === 1) return layout
  const order = panesOf(layout)
  const index = order.findIndex((pane) => pane.key === key)
  const heir = order[index + 1] ?? order[index - 1]
  const column = layout.columns[at.column]
  let columns: Column[]
  if (column.panes.length === 1) {
    const rest = layout.columns.filter((_, i) => i !== at.column)
    const taker = Math.min(at.column, rest.length - 1)
    columns = rest.map((other, i) =>
      i === taker ? { ...other, share: other.share + column.share } : other,
    )
  } else {
    const freed = column.panes[at.row].share
    const panes = column.panes.filter((_, i) => i !== at.row)
    const taker = Math.min(at.row, panes.length - 1)
    columns = layout.columns.map((other, i) =>
      i !== at.column
        ? other
        : {
            ...other,
            panes: panes.map((pane, j) =>
              j === taker ? { ...pane, share: pane.share + freed } : pane,
            ),
          },
    )
  }
  return {
    ...layout,
    columns,
    focused: layout.focused === key ? heir.key : layout.focused,
  }
}

/**
 * Puts a pane beside `target`: left or right opens a full-height column that
 * takes half of the target's column; top or bottom halves the target pane.
 * The pane keeps `pane.key` when it has one (a move), and is minted a new key
 * otherwise. Refuses — returns the layout it was given — past the limits.
 */
function insertPane(
  layout: PaneLayout,
  target: PaneKey,
  side: Side,
  pane: { key?: PaneKey; item: string },
): PaneLayout {
  const at = locate(layout, target)
  if (!at) return layout
  if (pane.key === undefined && isFull(layout)) return layout
  if (isHorizontal(side) && layout.columns.length >= paneLimits.maxColumns) return layout
  let nextKey = layout.nextKey
  const key = pane.key ?? nextKey++
  if (isHorizontal(side)) {
    const half = layout.columns[at.column].share / 2
    const column: Column = {
      key: nextKey++,
      share: half,
      panes: [{ key, item: pane.item, share: 1 }],
    }
    const halved = layout.columns.map((other, i) =>
      i === at.column ? { ...other, share: half } : other,
    )
    const index = side === "right" ? at.column + 1 : at.column
    return {
      columns: [...halved.slice(0, index), column, ...halved.slice(index)],
      focused: key,
      nextKey,
    }
  }
  const column = layout.columns[at.column]
  const half = column.panes[at.row].share / 2
  const halved = column.panes.map((other, j) =>
    j === at.row ? { ...other, share: half } : other,
  )
  const index = side === "bottom" ? at.row + 1 : at.row
  const panes = [
    ...halved.slice(0, index),
    { key, item: pane.item, share: half },
    ...halved.slice(index),
  ]
  return {
    columns: layout.columns.map((other, i) =>
      i === at.column ? { ...other, panes } : other,
    ),
    focused: key,
    nextKey,
  }
}

/** Opens a new pane showing `item` on `side` of `target`, and focuses it. */
export function splitPane(
  layout: PaneLayout,
  target: PaneKey,
  side: Side,
  item: string,
): PaneLayout {
  if (paneShowing(layout, item)) return layout
  return insertPane(layout, target, side, { item })
}

/** Two panes trade places; each place keeps its size. */
export function swapPanes(layout: PaneLayout, a: PaneKey, b: PaneKey): PaneLayout {
  const first = locate(layout, a)
  const second = locate(layout, b)
  if (!first || !second || a === b) return layout
  const paneA = layout.columns[first.column].panes[first.row]
  const paneB = layout.columns[second.column].panes[second.row]
  return mapPanes(layout, (pane) =>
    pane.key === a
      ? { ...paneB, share: pane.share }
      : pane.key === b
        ? { ...paneA, share: pane.share }
        : pane,
  )
}

/**
 * Moves a pane to a zone of another: the middle swaps the two, a side takes
 * it out and puts it there. The moved pane keeps its key and takes focus. A
 * move onto itself, or one the limits refuse, changes nothing.
 */
export function movePane(
  layout: PaneLayout,
  key: PaneKey,
  target: PaneKey,
  zone: Zone,
): PaneLayout {
  if (key === target) return layout
  const at = locate(layout, key)
  if (!at || !locate(layout, target)) return layout
  if (zone === "center") return focusPane(swapPanes(layout, key, target), key)
  const pane = layout.columns[at.column].panes[at.row]
  const without = removePane(layout, key)
  const placed = insertPane(without, target, zone, pane)
  return placed === without ? layout : placed
}

/**
 * The keyboard's move: trade places with the pane that way. At the side of a
 * stacked column, the pane steps out into a column of its own there.
 */
export function nudgePane(
  layout: PaneLayout,
  key: PaneKey,
  direction: Direction,
): PaneLayout {
  const at = locate(layout, key)
  if (!at) return layout
  const column = layout.columns[at.column]
  if (direction === "up" || direction === "down") {
    const neighbour = column.panes[at.row + (direction === "up" ? -1 : 1)]
    return neighbour ? focusPane(swapPanes(layout, key, neighbour.key), key) : layout
  }
  const beside = layout.columns[at.column + (direction === "left" ? -1 : 1)]
  if (beside) {
    const neighbour = beside.panes[Math.min(at.row, beside.panes.length - 1)]
    return focusPane(swapPanes(layout, key, neighbour.key), key)
  }
  const sibling = column.panes.find((pane) => pane.key !== key)
  if (!sibling) return layout
  return movePane(layout, key, sibling.key, direction)
}

/** Every column the same width, and every pane in a column the same height. */
export function equalizePanes(layout: PaneLayout): PaneLayout {
  const even = layout.columns.every(
    (column) => column.share === 1 && column.panes.every((pane) => pane.share === 1),
  )
  if (even) return layout
  return {
    ...layout,
    columns: layout.columns.map((column) => ({
      ...column,
      share: 1,
      panes: column.panes.map((pane) => ({ ...pane, share: 1 })),
    })),
  }
}

/**
 * Which pane is where, ignoring sizes: two layouts that differ only in how
 * the room is shared have the same shape. Motion plays when the shape
 * changes, not while an edge is dragged.
 */
export function layoutShape(layout: PaneLayout): string {
  return layout.columns
    .map((column) => `${column.key}:${column.panes.map((pane) => pane.key).join(",")}`)
    .join("|")
}
