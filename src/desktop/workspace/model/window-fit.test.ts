import { describe, expect, it } from "vitest"
import { paneLimits } from "../../split-panes/model/pane-layout"
import {
  clampColumn,
  columnWidth,
  foldToFit,
  railFits,
  sessionListLimits,
  sidebarLimits,
  treeSidebarLimits,
  type SideColumns,
} from "./window-fit"

const open: SideColumns = {
  sidebarOpen: true,
  sessionListOpen: true,
  sidebarWidth: 240,
  sessionListWidth: 312,
}

describe("side column widths", () => {
  it("holds a dragged width within the column's limits", () => {
    expect(clampColumn(50, sidebarLimits)).toBe(sidebarLimits.min)
    expect(clampColumn(900, sidebarLimits)).toBe(sidebarLimits.max)
    expect(clampColumn(250, sidebarLimits)).toBe(250)
  })

  it("draws a column no wider than its share of the window, and never under its minimum", () => {
    expect(columnWidth(320, sidebarLimits, 1440)).toBe(288)
    expect(columnWidth(240, sidebarLimits, 1440)).toBe(240)
    expect(columnWidth(320, sidebarLimits, 600)).toBe(sidebarLimits.min)
    expect(columnWidth(420, sessionListLimits, 1000)).toBe(260)
    expect(columnWidth(256, treeSidebarLimits, 1000)).toBe(256)
  })
})

describe("folding the side columns to fit", () => {
  it("keeps both when the panes fit beside them", () => {
    expect(foldToFit(open, 1440, 2)).toBe(open)
  })

  it("folds the sidebar first", () => {
    const need = 2 * paneLimits.minWidth + paneLimits.gutter
    const width = need + 16 + 312 + 8 + 100
    expect(foldToFit(open, width, 2)).toEqual({ ...open, sidebarOpen: false })
  })

  it("folds the list too when the sidebar alone is not enough", () => {
    expect(foldToFit(open, 700, 2)).toEqual({
      ...open,
      sidebarOpen: false,
      sessionListOpen: false,
    })
  })

  it("returns the same value when nothing is left to fold", () => {
    const folded = { ...open, sidebarOpen: false, sessionListOpen: false }
    expect(foldToFit(folded, 400, 2)).toBe(folded)
  })

  it("leaves closed columns closed and only folds what is open", () => {
    const sidebarOnly = { ...open, sessionListOpen: false, sessionListWidth: 0 }
    expect(foldToFit(sidebarOnly, 500, 1)).toEqual({ ...sidebarOnly, sidebarOpen: false })
  })
})

describe("the side rail gives way first", () => {
  const rail = 48
  // Exactly enough for one column of panes beside both side columns.
  const snug = paneLimits.minWidth + 2 * paneLimits.gutter + 240 + 8 + 312 + 8

  it("fits where standing beside the columns folds nothing", () => {
    expect(railFits(open, 1440, 1, rail)).toBe(true)
  })

  it("does not fit where it would fold a column the person has open", () => {
    expect(railFits(open, snug, 1, rail)).toBe(false)
    expect(railFits(open, snug + rail, 1, rail)).toBe(true)
  })

  it("fits beside a column the window folded already, without taking its room", () => {
    // The sidebar is folded at this width with or without the rail.
    const folded = snug - 100
    expect(foldToFit(open, folded, 1).sidebarOpen).toBe(false)
    // The list still has its room beside the rail, so the rail fits…
    expect(railFits(open, folded, 1, rail)).toBe(true)
    // …and where the rail would cost the list its room too, it does not.
    const listSnug = paneLimits.minWidth + 2 * paneLimits.gutter + 312 + 8
    expect(railFits(open, listSnug, 1, rail)).toBe(false)
  })

  it("fits wherever the person has the side columns closed", () => {
    const closed = { ...open, sidebarOpen: false, sessionListOpen: false }
    expect(railFits(closed, 560, 1, rail)).toBe(true)
  })
})
