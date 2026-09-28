import { describe, expect, it } from "vitest"
import {
  chosen,
  collapseSnap,
  draggedEdge,
  drawn,
  fitted,
  type SideColumn,
} from "./side-column"

const open: SideColumn = { open: true, folded: false }
const hidden: SideColumn = { open: false, folded: false }

describe("a side column", () => {
  it("folds for room only when the person has it open, and comes back with the room", () => {
    const folded = fitted(open, false)
    expect(drawn(folded)).toBe(false)
    expect(folded.open).toBe(true)
    expect(drawn(fitted(folded, true))).toBe(true)
    expect(fitted(hidden, false)).toBe(hidden)
  })

  it("never brings back a column the person hid", () => {
    expect(drawn(fitted(hidden, true))).toBe(false)
  })

  it("lets the person's choice win at once, a fold for room included", () => {
    const folded = fitted(open, false)
    expect(drawn(chosen(folded))).toBe(true)
    expect(chosen(folded).folded).toBe(false)
    expect(drawn(chosen(open))).toBe(false)
    expect(chosen(open, true)).toBe(open)
  })
})

describe("dragging a column's edge", () => {
  const limits = { min: 260, max: 420 }

  it("resists at the narrowest, then folds the column once dragged far enough past it", () => {
    expect(draggedEdge(312, -40, limits)).toEqual({ open: true, width: 272 })
    expect(draggedEdge(312, -52 - collapseSnap + 1, limits)).toEqual({
      open: true,
      width: 260,
    })
    expect(draggedEdge(312, -52 - collapseSnap - 1, limits)).toEqual({
      open: false,
      width: 260,
    })
    expect(draggedEdge(312, 400, limits)).toEqual({ open: true, width: 420 })
  })

  it("opens a folded column once dragged back out far enough", () => {
    expect(draggedEdge(null, collapseSnap - 1, limits).open).toBe(false)
    expect(draggedEdge(null, collapseSnap, limits)).toEqual({ open: true, width: 260 })
  })
})
