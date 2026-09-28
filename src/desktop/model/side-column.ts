/**
 * A column at the window's side — a sidebar, a session list — shown or
 * hidden two ways: by the person's choice, and by the window folding it for
 * room. The two are kept apart, so a column the window folded comes back on
 * its own once there is room again, while one the person hid stays hidden.
 * The workspace's side columns and Settings' sidebar both follow this.
 *
 * | column       | event                    | next                      |
 * | ------------ | ------------------------ | ------------------------- |
 * | any          | the person shows it      | open, not folded          |
 * | any          | the person hides it      | closed, not folded        |
 * | open         | no room for it           | open, folded              |
 * | open, folded | room again               | open, not folded (drawn)  |
 * | closed       | room or none             | closed                    |
 *
 * The person's choice wins at once: showing a folded column draws it, until
 * the room changes again.
 */
export interface SideColumn {
  /** The person's choice. */
  readonly open: boolean
  /** Folded by the window for room, whatever the person chose. */
  readonly folded: boolean
}

/** Whether the column is drawn: chosen open, and not folded for room. */
export function drawn(column: SideColumn): boolean {
  return column.open && !column.folded
}

/** The person shows the column (`open`), hides it, or — with neither — turns what is drawn over. */
export function chosen(column: SideColumn, open: boolean = !drawn(column)): SideColumn {
  return column.open === open && !column.folded ? column : { open, folded: false }
}

/** The window has room for the column, or not: only an open column folds. */
export function fitted(column: SideColumn, room: boolean): SideColumn {
  const folded = column.open && !room
  return folded === column.folded ? column : { ...column, folded }
}

/** How far past its narrowest a column's edge is dragged before it folds away, and back out before it opens. */
export const collapseSnap = 40

/**
 * Where a drag of a column's edge leaves the column: open at `width` —
 * held at its narrowest (`min`) while the drag resists — or folded away once
 * dragged `collapseSnap` past it. A drag that starts on a folded column
 * (`from` null) opens it once it has come `collapseSnap` out. Folding or
 * opening this way is the person's own choice, as a toggle is.
 */
export function draggedEdge(
  from: number | null,
  delta: number,
  { min, max }: { min: number; max: number },
): { open: boolean; width: number } {
  if (from === null) return { open: delta >= collapseSnap, width: min }
  const width = from + delta
  if (width < min - collapseSnap) return { open: false, width: min }
  return { open: true, width: Math.min(Math.max(width, min), max) }
}
